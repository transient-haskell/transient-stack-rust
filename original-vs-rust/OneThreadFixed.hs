{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch)
import qualified System.Timeout as T
import Data.IORef

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 3000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' returned nothing in 3 s)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "O4 oneThread: tasks shorter than the interval: all finish" $ do
    n <- liftIO $ newIORef (0 :: Int)
    x <- oneThread $ waitEvents (do threadDelay 80000
                                    k <- atomicModifyIORef n (\a -> (a+1, a+1))
                                    return k)
    when (x > 4) $ empty
    async (threadDelay 20000)
    say ("  process " ++ show x)

  run "O5 oneThread over option (react): no producer of its own" $ do
    x <- oneThread $ option "go" "go"
    async (threadDelay 300000)
    say ("  process (option) " ++ show x)
