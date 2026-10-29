{-# LANGUAGE ScopedTypeVariables, OverloadedStrings #-}
import Transient.Internals
import Transient.Loggable
import Transient.Parse
import qualified Data.ByteString.Lazy.Char8 as BL
import qualified Data.ByteString.Char8 as BC
import qualified Data.Text as T
import Data.ByteString.Builder
import Control.Monad.IO.Class

ser :: Loggable a => String -> a -> IO ()
ser label x = putStrLn (label ++ " => " ++ show (BL.unpack (toLazyByteString (serialize x))))

-- what `deserialize` reads from a text (up to the first undoubled `/`), and what is left
des :: forall a. (Loggable a, Show a) => String -> a -> IO ()
des input _ = do
  (r, _) <- runTransient $ withParseString (BL.pack input) $ do
              (x :: a) <- deserialize
              rest <- giveParseString
              return (x, BL.unpack rest)
  putStrLn ("  read " ++ show input ++ " => " ++ show r)

main = do
  putStrLn "-- reading of edge cases"
  des "[]/x" ("" :: String)
  des "'\\233'/y" 'a'
  des "'\\n'/y" 'a'
  des "'/'/y" 'a'
  des "'\\47'/y" 'a'
  des "a/b" (T.pack "")
  des "[1,2,3]/x" ([] :: [Int])
  des "[\"a\",\"b//c\"]/x" ([] :: [String])
  putStrLn "-- a String that is literally []"
  ser "String \"[]\"" ("[]" :: String)
